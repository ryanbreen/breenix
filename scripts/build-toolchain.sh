#!/usr/bin/env bash
# Build the development bundle and reference corpus for one ELF target.
# The producer supplies absolute BREENIX_TOOLCHAIN_CLANG and BREENIX_TOOLCHAIN_LLD
# paths with their corresponding *_SHA256 pins.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
arch=
output=
archive=
while [ "$#" -gt 0 ]; do
    [ "$#" -ge 2 ] || { echo "$1 needs a value" >&2; exit 2; }
    case "$1" in
        --arch) arch=$2 ;;
        --output) output=$2 ;;
        --libc) archive=$2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
    shift 2
done
case "$arch" in
    x86_64) c_target=x86_64-breenix-c; binaries="$root/userspace/programs" ;;
    aarch64) c_target=aarch64-breenix; binaries="$root/userspace/programs/aarch64" ;;
    *) echo "build-toolchain: --arch must be x86_64 or aarch64" >&2; exit 2 ;;
esac
[ -n "$output" ] && [ ! -e "$output" ] && [ ! -L "$output" ] || {
    echo "build-toolchain: --output must name a new directory" >&2
    exit 2
}
clang=${BREENIX_TOOLCHAIN_CLANG:?set the pinned absolute Clang path}
lld=${BREENIX_TOOLCHAIN_LLD:?set the pinned absolute lld path}
clang_pin=${BREENIX_TOOLCHAIN_CLANG_SHA256:?set the Clang executable SHA-256}
lld_pin=${BREENIX_TOOLCHAIN_LLD_SHA256:?set the lld executable SHA-256}

# Keep a partial build private. Put the published directory on the same
# filesystem so the suite staging path can only see it after it is complete.
work=$(mktemp -d "$(dirname "$output")/.toolchain-build.XXXXXX")
trap 'rm -rf "$work"' EXIT
work=$(cd "$work" && pwd)
commit=$(git -C "$root" rev-parse HEAD)
guest="$work/bundle/root"
dev="$guest/opt/breenix-dev/25.2"
corpus="$guest/usr/share/breenix/toolchain"
mkdir -p "$dev/lib" "$corpus" "$guest/sbin"

if [ -z "$archive" ]; then
    # Metal's ordinary userspace artifact cache does not export libc.a.
    # Build the C ABI archive for this fixture in its private target directory.
    (
        cd "$root/libs/libbreenix-libc"
        CARGO_ENCODED_RUSTFLAGS= RUSTFLAGS= cargo build --release \
            --target "$root/$c_target.json" --target-dir "$work/libc-target"
    )
    archive="$work/libc-target/$c_target/release/libc.a"
    python3 "$root/scripts/libc-clean-archive.py" --archive "$archive" \
        --tool-dir "$(dirname "$clang")"
fi

install -m 0644 "$archive" "$dev/lib/libc.a"
install -m 0644 "$root/userspace/programs/linker.ld" "$dev/linker.ld"
install -m 0755 "$binaries/suite-toolchain.elf" "$guest/sbin/suite-toolchain"
mkdir -p "$dev/include" "$dev/include-support"
cp -R "$root/libs/libbreenix-libc/include/." "$dev/include/"
cp -R "$root/toolchain/include/." "$dev/include-support/"

# Check the source and expectation set before compiling. The same list is
# built into suite-toolchain, independently of the generated bundle manifest.
python3 "$root/scripts/toolchain-image.py" check-corpus "$root/toolchain/corpus"
while IFS= read -r name; do
    install -m 0644 "$root/toolchain/corpus/$name.c" "$corpus/$name.c"
    install -m 0644 "$root/toolchain/corpus/$name.expect" "$corpus/$name.expect"
    "$root/scripts/toolchain-ref-cc" --arch "$arch" \
        --clang "$clang" --clang-sha256 "$clang_pin" --lld "$lld" --lld-sha256 "$lld_pin" \
        --input-root "$corpus" --input-root "$dev/lib" --archive "$dev/lib/libc.a" \
        -o "$corpus/$name.elf" --inventory "$corpus/$name.inventory.json" "$corpus/$name.c"
    chmod 0755 "$corpus/$name.elf"
done < "$root/toolchain/corpus/cases"

python3 "$root/scripts/toolchain-build-bas.py" "$arch" "$work/bas" "$dev" "$corpus"

python3 "$root/scripts/toolchain-image.py" seal "$work/bundle" "$commit" "$arch"
mv "$work/bundle" "$output"
