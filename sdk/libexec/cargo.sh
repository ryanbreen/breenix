# cargo for a Breenix target with the SDK's prebuilt std (RUSTAPP lane). bin/<arch>-breenix-cargo
# sources this with SDK, SDK_TOOL and SDK_ARCH set, sdk-common.sh loaded and cargo's arguments
# as "$@"; it answers --version itself.
#
# It pins the SDK's nightly (and refuses any other rustc), names the target through the SDK's
# target specs, links against the SDK's sysroot, libc and linker script, and resolves crates.io
# dependencies (libbreenix) from the SDK's vendor directory, offline.

_cargo_target="$SDK_ARCH-breenix"
_cargo_share="$SDK/share/breenix-sdk"

case "${1-}" in
    +*) sdk_die "the SDK builds with $(sdk_setting TOOLCHAIN) only; drop '$1'" ;;
esac
sdk_use_toolchain
_cargo_want="$(sdk_setting RUSTC_COMMIT)"
_cargo_have="$("${RUSTC:-rustc}" -vV 2>/dev/null | sed -n 's/^commit-hash: //p')" || _cargo_have=
if [ "$_cargo_have" != "$_cargo_want" ]; then
    sdk_die "the SDK's std was built by rustc $RUSTUP_TOOLCHAIN (commit $_cargo_want), but ${RUSTC:-rustc} is ${_cargo_have:-not runnable}; install it with: rustup toolchain install $RUSTUP_TOOLCHAIN"
fi

# _cargo_toml VALUE: VALUE as a TOML basic string.
_cargo_toml() {
    printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g')"
}

if [ "$SDK_ARCH" = x86_64 ]; then
    # Rust on x86_64 is soft-float; its libc.a lives in the sysroot beside std.
    _cargo_libdir="$SDK/lib/rust/lib/rustlib/$_cargo_target/lib"
else
    _cargo_libdir="$SDK/lib/$_cargo_target"
fi
_cargo_flags="$(_cargo_toml "--sysroot=$SDK/lib/rust")"
_cargo_flags="$_cargo_flags, $(_cargo_toml "-Lnative=$_cargo_libdir")"
_cargo_flags="$_cargo_flags, $(_cargo_toml "-Clink-arg=-T$_cargo_share/ldscripts/breenix.ld")"
_cargo_flags="$_cargo_flags, \"-Cdefault-linker-libraries=no\""
_cargo_flags="$_cargo_flags, $(_cargo_toml "--remap-path-prefix=$SDK=/breenix-sdk")"

# RUSTFLAGS in the environment would replace every configured flag, the SDK's too, so fold
# them into the target's list instead.
set -f
if [ -n "${CARGO_ENCODED_RUSTFLAGS-}" ]; then
    _cargo_ifs="$IFS"
    IFS="$(printf '\037')"
    for _cargo_flag in $CARGO_ENCODED_RUSTFLAGS; do
        _cargo_flags="$_cargo_flags, $(_cargo_toml "$_cargo_flag")"
    done
    IFS="$_cargo_ifs"
elif [ -n "${RUSTFLAGS-}" ]; then
    for _cargo_flag in $RUSTFLAGS; do
        _cargo_flags="$_cargo_flags, $(_cargo_toml "$_cargo_flag")"
    done
fi
set +f
unset CARGO_ENCODED_RUSTFLAGS RUSTFLAGS

export RUST_TARGET_PATH="$_cargo_share/targets"
export CARGO_BUILD_TARGET="$_cargo_target"
exec cargo \
    --config "target.$_cargo_target.rustflags=[$_cargo_flags]" \
    --config 'source.crates-io.replace-with="breenix-sdk"' \
    --config "source.breenix-sdk.directory=$(_cargo_toml "$_cargo_share/vendor")" \
    "$@"
