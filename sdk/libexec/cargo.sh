#!/bin/sh
# cargo for a Breenix target with the SDK's prebuilt std (RUSTAPP). bin/<arch>-breenix-cargo
# runs it as
#
#   cargo.sh SDK_ROOT ARCH [cargo arguments...]
#
# It pins the SDK's nightly (and refuses any other rustc), names the target through the
# SDK's target specs, links against the SDK's sysroot, libc and linker script, and
# resolves crates.io dependencies (libbreenix) from the SDK's vendor directory, offline.
set -eu

root=$1
arch=$2
shift 2
share=$root/share/breenix-sdk
target=$arch-breenix
tool=$target-cargo

read -r _ version commit < "$root/VERSION"
setting() {
    sed -n "s/^$1=//p" "$share/sdk.env"
}
toolchain=$(setting TOOLCHAIN)
rustc_commit=$(setting RUSTC_COMMIT)
export RUSTUP_TOOLCHAIN="$toolchain"

case ${1-} in
--version | -V)
    echo "breenix-sdk $version $arch $commit"
    cargo --version
    exit
    ;;
+*)
    echo "$tool: the SDK builds with $toolchain only; drop '$1'" >&2
    exit 2
    ;;
esac

have=$("${RUSTC:-rustc}" -vV 2>/dev/null | sed -n 's/^commit-hash: //p') || have=
if [ "$have" != "$rustc_commit" ]; then
    echo "$tool: the SDK's std was built by rustc $toolchain (commit $rustc_commit), but ${RUSTC:-rustc} is ${have:-not runnable}" >&2
    echo "$tool: install it with: rustup toolchain install $toolchain" >&2
    exit 1
fi

# A TOML basic string holding $1.
toml() {
    printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g')"
}

if [ "$arch" = x86_64 ]; then
    # Rust on x86_64 is soft-float; its libc.a lives in the sysroot beside std.
    libdir=$root/lib/rust/lib/rustlib/$target/lib
else
    libdir=$root/lib/$target
fi
flags="$(toml "--sysroot=$root/lib/rust")"
flags="$flags, $(toml "-Lnative=$libdir")"
flags="$flags, $(toml "-Clink-arg=-T$share/ldscripts/breenix.ld")"
flags="$flags, \"-Clink-arg=--allow-multiple-definition\", \"-Cdefault-linker-libraries=no\""
flags="$flags, $(toml "--remap-path-prefix=$root=/breenix-sdk")"

# RUSTFLAGS in the environment would replace every configured flag, the SDK's too, so
# fold them into the target's list instead.
if [ -n "${CARGO_ENCODED_RUSTFLAGS-}" ]; then
    old_ifs=$IFS
    IFS=$(printf '\037')
    set -f
    for flag in $CARGO_ENCODED_RUSTFLAGS; do
        flags="$flags, $(toml "$flag")"
    done
    set +f
    IFS=$old_ifs
elif [ -n "${RUSTFLAGS-}" ]; then
    set -f
    for flag in $RUSTFLAGS; do
        flags="$flags, $(toml "$flag")"
    done
    set +f
fi
unset CARGO_ENCODED_RUSTFLAGS RUSTFLAGS

export RUST_TARGET_PATH="$share/targets"
export CARGO_BUILD_TARGET="$target"
exec cargo \
    --config "target.$target.rustflags=[$flags]" \
    --config 'source.crates-io.replace-with="breenix-sdk"' \
    --config "source.breenix-sdk.directory=$(toml "$share/vendor")" \
    "$@"
