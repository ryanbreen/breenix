#!/usr/bin/env bash
# Build the Breenix Developer SDK for aarch64 and x86_64 from this checkout.
#
#   scripts/build-sdk.sh --prefix DIR [--fetch]
#
# DIR must not exist or be empty. The build is offline: --fetch (once per host) lets
# cargo and git reach the network so a cold cache is filled, and the next build needs
# none. The SDK is versioned by the commit it is built from, so the tree must be clean.
#
# The commit's sources are archived into a fixed build root, /tmp/breenix-sdk-build/<commit>,
# together with the pinned Rust library from BREENIX_RUST_FORK_LIBRARY (default:
# rust-fork/library), a git checkout holding the pin or a verified snapshot. A fixed root
# keeps cargo's path-derived crate hashes, and so the output, identical across checkouts.
# The stages in sdk/build/stages/ then fill <root>/stage in order, the result is normalized
# and scanned for local paths, and it is moved to DIR.
#
# Host prerequisites: git, bash, python3 >= 3.9, rustup with the toolchain named by
# rust-toolchain.toml (components rust-src and llvm-tools). Nothing in a kernel build or
# a boot runs this script; only scripts/sdk-suite.py does.
set -euo pipefail

usage() {
    echo "usage: $0 --prefix DIR [--fetch]" >&2
    exit 2
}

PREFIX=
FETCH=0
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) [ $# -ge 2 ] || usage; PREFIX="$2"; shift 2 ;;
        --fetch) FETCH=1; shift ;;
        *) usage ;;
    esac
done
[ -n "$PREFIX" ] || usage

die() {
    echo "build-sdk: $*" >&2
    exit 1
}

CHECKOUT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
[ -z "$(git -C "$CHECKOUT" status --porcelain)" ] ||
    die "the checkout has uncommitted changes; the SDK is built from a commit"
COMMIT="$(git -C "$CHECKOUT" rev-parse HEAD)"
case "$PREFIX" in /*) ;; *) PREFIX="$PWD/$PREFIX" ;; esac
if [ -e "$PREFIX" ] && [ -n "$(ls -A "$PREFIX")" ]; then
    die "$PREFIX is not empty"
fi

FETCH_ARG=()
[ "$FETCH" = 0 ] || FETCH_ARG=(--fetch)
# The builder's home, which HOME need not name (the suite gives builds a home of their own).
REAL_HOME="$(python3 -c 'import os, pwd; print(pwd.getpwuid(os.getuid()).pw_dir)')"

BASE=/tmp/breenix-sdk-build
LOCK="$BASE/$COMMIT.lock"
# One build of a commit at a time: re-run this script holding an exclusive lock on the
# commit's lock file (macOS has no flock(1)). The lock lives as long as the open file.
if [ "${BREENIX_SDK_BUILD_LOCK:-}" != "$LOCK" ]; then
    mkdir -p "$BASE"
    exec python3 -c '
import fcntl, os, sys
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT, 0o644)
fcntl.flock(fd, fcntl.LOCK_EX)
os.set_inheritable(fd, True)
os.environ["BREENIX_SDK_BUILD_LOCK"] = sys.argv[1]
os.execvp(sys.argv[2], sys.argv[2:])
' "$LOCK" bash "${BASH_SOURCE[0]}" --prefix "$PREFIX" "${FETCH_ARG[@]+"${FETCH_ARG[@]}"}"
fi

VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$CHECKOUT/sdk/sdk.toml")"
[ -n "$VERSION" ] || die "sdk/sdk.toml names no version"
SOURCE_DATE_EPOCH="$(git -C "$CHECKOUT" log -1 --format=%ct HEAD)"
export SOURCE_DATE_EPOCH LC_ALL=C TZ=UTC
umask 022

# The pinned toolchain, named explicitly: the build root holds no rust-toolchain.toml.
SDK_TOOLCHAIN="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$CHECKOUT/rust-toolchain.toml")"
[ -n "$SDK_TOOLCHAIN" ] || die "rust-toolchain.toml names no channel"
export RUSTUP_TOOLCHAIN="$SDK_TOOLCHAIN"
RUSTC_VV="$(rustc -vV)" || die "rustc for $SDK_TOOLCHAIN does not run (rustup toolchain install $SDK_TOOLCHAIN)"
HOST="$(printf '%s\n' "$RUSTC_VV" | sed -n 's/^host: //p')"
RUSTC_COMMIT="$(printf '%s\n' "$RUSTC_VV" | sed -n 's/^commit-hash: //p')"
RUST_SYSROOT="$(rustc --print sysroot)"
SDK_LLVM_TOOLS="$RUST_SYSROOT/lib/rustlib/$HOST/bin"
[ -x "$SDK_LLVM_TOOLS/llvm-nm" ] || die "$SDK_TOOLCHAIN has no llvm-tools (rustup component add llvm-tools --toolchain $SDK_TOOLCHAIN)"
[ -d "$RUST_SYSROOT/lib/rustlib/src/rust/library" ] ||
    die "$SDK_TOOLCHAIN has no rust-src (rustup component add rust-src --toolchain $SDK_TOOLCHAIN)"

# The pinned Rust library.
FORK_COMMIT="$(sed -n 's/^commit = "\([0-9a-f]\{40\}\)"$/\1/p' "$CHECKOUT/rust-fork.toml")"
[ -n "$FORK_COMMIT" ] || die "rust-fork.toml pins no full commit"
FORK_REPO="$(sed -n 's/^repo = "\(.*\)"$/\1/p' "$CHECKOUT/rust-fork.toml")"
FORK_LIBRARY="${BREENIX_RUST_FORK_LIBRARY:-$CHECKOUT/rust-fork/library}"
[ -d "$FORK_LIBRARY" ] || die "no pinned Rust library at $FORK_LIBRARY (set BREENIX_RUST_FORK_LIBRARY or run scripts/rust-fork.sh sync)"
FORK="$(cd "$FORK_LIBRARY/.." && pwd -P)"

ROOT_DIR="$BASE/$COMMIT"
rm -rf "$ROOT_DIR"
mkdir -p "$ROOT_DIR"
ROOT_REAL="$(cd "$ROOT_DIR" && pwd -P)"
if [ "${BREENIX_SDK_KEEP_BUILD:-0}" != 1 ]; then
    trap 'rm -rf "$ROOT_DIR"' EXIT
fi

echo "build-sdk: breenix-sdk $VERSION $COMMIT, toolchain $SDK_TOOLCHAIN, Rust library $FORK_COMMIT"

# The commit's sources the stages read.
SOURCES=""
for path in libs userspace/programs/linker.ld aarch64-breenix.json x86_64-breenix.json \
            x86_64-breenix-c.json rust-fork.toml rust-toolchain.toml sdk docs/sdk \
            scripts/libc-clean-archive.py; do
    if git -C "$CHECKOUT" cat-file -e "HEAD:$path" 2>/dev/null; then
        SOURCES="$SOURCES $path"
    fi
done
# shellcheck disable=SC2086
git -C "$CHECKOUT" archive --format=tar HEAD $SOURCES | tar -xf - -C "$ROOT_DIR"

# The pinned library goes to <root>/rust-fork/library, where its ../../libs/libc patch
# reaches this commit's libs/libc.
mkdir -p "$ROOT_DIR/rust-fork"
if [ -f "$FORK/.rust-fork-snapshot.json" ]; then
    python3 - "$FORK" "$FORK_COMMIT" <<'PY' || die "the Rust library snapshot at $FORK does not hold the pin"
import hashlib, json, sys
from pathlib import Path
fork, pin = Path(sys.argv[1]), sys.argv[2]
meta = json.loads((fork / '.rust-fork-snapshot.json').read_text())
digest = hashlib.sha256()
for path in sorted(fork.rglob('*')):
    if path.is_file() and path.name != '.rust-fork-snapshot.json':
        digest.update(str(path.relative_to(fork)).encode() + b'\0' + path.read_bytes() + b'\0')
sys.exit(0 if meta.get('commit') == pin and meta.get('digest') == digest.hexdigest() else 1)
PY
    cp -R "$FORK/library" "$ROOT_DIR/rust-fork/library"
else
    if ! git -C "$FORK" cat-file -e "$FORK_COMMIT^{commit}" 2>/dev/null; then
        [ "$FETCH" = 1 ] || die "the Rust fork at $FORK lacks $FORK_COMMIT; run with --fetch once"
        git -C "$FORK" fetch --quiet --no-write-fetch-head "$FORK_REPO" "$FORK_COMMIT"
    fi
    git -C "$FORK" archive --format=tar "$FORK_COMMIT" library | tar -xf - -C "$ROOT_DIR/rust-fork"
    git -C "$FORK" ls-tree -r "$FORK_COMMIT" library | while read -r mode kind sha path; do
        [ "$mode" = 160000 ] || continue
        git -C "$FORK/$path" cat-file -e "$sha^{commit}" 2>/dev/null ||
            die "the Rust fork's submodule $path lacks $sha; run scripts/rust-fork.sh sync"
        git -C "$FORK/$path" archive --format=tar --prefix="$path/" "$sha" | tar -xf - -C "$ROOT_DIR/rust-fork"
    done
fi

SDK_STAGE="$ROOT_DIR/stage"
mkdir -p "$SDK_STAGE"

# Cargo takes configuration from its home, from every directory above the crate and from
# CARGO_* variables, and any of them (a [build] target-dir, [profile.release] settings, a
# rustc wrapper) could move or change what the SDK is built from without changing its
# version. The stages get a cargo home holding only links to the builder's crate caches, none
# of the builder's cargo or rustc settings but the job count, and no directory above the
# build root may hold a cargo configuration.
BUILDER_CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
mkdir -p "$ROOT_DIR/cargo-home"
for cache in registry git; do
    mkdir -p "$BUILDER_CARGO_HOME/$cache"
    ln -s "$BUILDER_CARGO_HOME/$cache" "$ROOT_DIR/cargo-home/$cache"
done
for top in "$ROOT_DIR" "$ROOT_REAL"; do
    dir="$top"
    while [ "$dir" != / ]; do
        dir="$(dirname "$dir")"
        for config in "$dir/.cargo/config" "$dir/.cargo/config.toml"; do
            [ ! -e "$config" ] || die "$config would configure the SDK's cargo builds; move it aside"
        done
    done
done
for var in $(compgen -e); do
    case "$var" in
        CARGO_BUILD_JOBS) ;;
        CARGO_*) unset "$var" ;;
    esac
done
unset RUSTC RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER RUSTDOC RUSTFLAGS RUSTDOCFLAGS BREENIX_USERSPACE_REMAP
export CARGO_HOME="$ROOT_DIR/cargo-home" SDK_BUILDER_CARGO_HOME="$BUILDER_CARGO_HOME"

if [ "$FETCH" = 1 ]; then
    CARGO_NET_OFFLINE=false
else
    CARGO_NET_OFFLINE=true
fi
export CARGO_NET_OFFLINE
export SDK_ROOT="$ROOT_DIR" SDK_ROOT_REAL="$ROOT_REAL" SDK_STAGE SDK_COMMIT="$COMMIT" SDK_VERSION="$VERSION" \
    SDK_TOOLCHAIN SDK_LLVM_TOOLS SDK_RUSTC_COMMIT="$RUSTC_COMMIT" SDK_RUST_SYSROOT="$RUST_SYSROOT" \
    SDK_FORK_COMMIT="$FORK_COMMIT" SDK_REAL_HOME="$REAL_HOME"

for stage in "$ROOT_DIR"/sdk/build/stages/*.sh; do
    echo "build-sdk: stage $(basename "$stage")"
    (cd "$ROOT_DIR" && bash "$stage") || die "stage $(basename "$stage") failed"
done

# Normalize: programs 0755, everything else 0644, directories 0755.
find "$SDK_STAGE" -type d -exec chmod 0755 {} +
find "$SDK_STAGE" -type f -exec chmod 0644 {} +
for dir in "$SDK_STAGE/bin" "$SDK_STAGE/share/breenix-sdk/libexec"; do
    [ -d "$dir" ] && find "$dir" -type f -exec chmod 0755 {} +
done

# No file may name the build's directories, the checkout, the Rust fork or the home.
python3 - "$SDK_STAGE" "$ROOT_DIR" "$ROOT_REAL" "$CHECKOUT" "$FORK" "$HOME" "$REAL_HOME" <<'PY' || die "the SDK names local paths"
import sys
from pathlib import Path
stage, needles = Path(sys.argv[1]), sorted({n.encode() for n in sys.argv[2:] if n not in ('', '/')})
bad = []
for path in sorted(stage.rglob('*')):
    if path.is_file() and not path.is_symlink():
        data = path.read_bytes()
        bad += [f'{path.relative_to(stage)} names {n.decode()}' for n in needles if n in data]
for line in bad[:20]:
    print('build-sdk: ' + line, file=sys.stderr)
sys.exit(1 if bad else 0)
PY

if [ -d "$PREFIX" ]; then
    rmdir "$PREFIX"
fi
mkdir -p "$(dirname "$PREFIX")"
mv "$SDK_STAGE" "$PREFIX"
echo "build-sdk: built breenix-sdk $VERSION $COMMIT in $PREFIX"
