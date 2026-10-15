# Helpers for the SDK build's stages (sdk/build/stages/*.sh). Each stage sources this file:
#
#   . "$SDK_ROOT/sdk/build/lib.sh"
#
# scripts/build-sdk.sh runs every stage from the build root with these set:
#   SDK_ROOT         the build root, holding the commit's sources and rust-fork/library
#   SDK_ROOT_REAL    the same directory with symlinks resolved (/private/tmp/... on macOS)
#   SDK_STAGE        the SDK being assembled; a stage writes only its own files here
#   SDK_COMMIT       the commit being built (40 hex)
#   SDK_VERSION      sdk/sdk.toml's version
#   SDK_TOOLCHAIN    the pinned Rust toolchain, also exported as RUSTUP_TOOLCHAIN
#   SDK_RUSTC_COMMIT that toolchain's rustc commit-hash
#   SDK_RUST_SYSROOT that toolchain's sysroot
#   SDK_LLVM_TOOLS   its LLVM tools (llvm-nm, llvm-ar, llvm-objcopy, ...), the only ones that
#                    read rustc's own bitcode
#   SDK_FORK_COMMIT  the Rust library pin (rust-fork.toml)
#   SDK_REAL_HOME    the builder's home directory, whatever HOME says
#   CARGO_NET_OFFLINE true unless build-sdk.sh was given --fetch
# and LC_ALL=C, TZ=UTC, SOURCE_DATE_EPOCH (the commit's time) and umask 022.
set -euo pipefail

sdk_say() {
    echo "build-sdk: $*"
}

sdk_die() {
    echo "build-sdk: $*" >&2
    exit 1
}

# Copy a file into the stage, making its directory: sdk_install SOURCE STAGE-RELATIVE-PATH
sdk_install() {
    mkdir -p "$(dirname "$SDK_STAGE/$2")"
    cp "$1" "$SDK_STAGE/$2"
}

# The --remap-path-prefix flags every rustc gets, one per line. rustc applies the last
# matching mapping, so the most specific directories come last.
sdk_remap_flags() {
    local cargo_home="${CARGO_HOME:-$SDK_REAL_HOME/.cargo}"
    local rustup_home="${RUSTUP_HOME:-$SDK_REAL_HOME/.rustup}"
    echo "--remap-path-prefix=$SDK_REAL_HOME=/home"
    [ "$HOME" = "$SDK_REAL_HOME" ] || echo "--remap-path-prefix=$HOME=/home"
    echo "--remap-path-prefix=$rustup_home=/rustup"
    echo "--remap-path-prefix=$cargo_home=/cargo"
    echo "--remap-path-prefix=$SDK_RUST_SYSROOT=/rust-sysroot"
    echo "--remap-path-prefix=$SDK_RUST_SYSROOT/lib/rustlib/src/rust=/rustc/$SDK_RUSTC_COMMIT"
    echo "--remap-path-prefix=$SDK_ROOT=/breenix-sdk"
    [ "$SDK_ROOT_REAL" = "$SDK_ROOT" ] || echo "--remap-path-prefix=$SDK_ROOT_REAL=/breenix-sdk"
}

# Run cargo in a crate directory with the remaps plus any extra rustc flags:
#   sdk_cargo DIR [--rustflag FLAG]... -- CARGO-ARGUMENTS...
# Output is prefixed so the build log says where it came from.
sdk_cargo() {
    local dir="$1"
    shift
    local flags
    flags="$(sdk_remap_flags)"
    while [ "${1:-}" = --rustflag ]; do
        flags="$flags"$'\n'"$2"
        shift 2
    done
    [ "${1:-}" = -- ] && shift
    (
        set -o pipefail
        cd "$dir"
        CARGO_ENCODED_RUSTFLAGS="$(printf '%s' "$flags" | tr '\n' '\037')" cargo "$@" 2>&1 |
            sed 's/^/  /'
    )
}

# A directory of the pinned toolchain's LLVM tools under the names scripts/libc-clean-archive.py
# runs (llvm-readobj acts as llvm-readelf when called by that name).
sdk_tool_dir() {
    local dir="$SDK_ROOT/.llvm-tools"
    if [ ! -d "$dir" ]; then
        mkdir -p "$dir"
        local tool
        for tool in llvm-nm llvm-ar llvm-objcopy; do
            ln -s "$SDK_LLVM_TOOLS/$tool" "$dir/$tool"
        done
        ln -s "$SDK_LLVM_TOOLS/llvm-readobj" "$dir/llvm-readelf"
    fi
    echo "$dir"
}

# Strip compiler_builtins' libm math (a port of musl's) from an archive, in place.
sdk_clean_archive() {
    python3 "$SDK_ROOT/scripts/libc-clean-archive.py" --archive "$1" --tool-dir "$(sdk_tool_dir)"
}
