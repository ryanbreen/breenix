# Shell code the SDK's bin/ entries share (POSIX sh). An entry finds the SDK's root from
# its own path, sets SDK to it and SDK_TOOL to its own name, then sources this file:
#
#   . "$SDK/share/breenix-sdk/libexec/sdk-common.sh"
#
# Host tools are found, never bundled: clang (BREENIX_CC_CLANG, then Homebrew's or
# linuxbrew's llvm, then PATH; macOS puts Apple's clang first on PATH) and the pinned Rust
# toolchain named by share/breenix-sdk/sdk.env, whose lld links every program.

sdk_die() {
    printf '%s: %s\n' "${SDK_TOOL:-breenix-sdk}" "$*" >&2
    exit 1
}

# sdk_setting NAME: NAME's value in share/breenix-sdk/sdk.env.
sdk_setting() {
    sed -n "s/^$1=//p" "$SDK/share/breenix-sdk/sdk.env"
}

# sdk_version_line WHAT: "breenix-sdk <version> <WHAT> <commit>", the first line of
# every tool's --version.
sdk_version_line() {
    read -r _sdk_name _sdk_version _sdk_commit < "$SDK/VERSION" || sdk_die "the SDK has no VERSION"
    printf 'breenix-sdk %s %s %s\n' "$_sdk_version" "$1" "$_sdk_commit"
}

# Select the pinned Rust toolchain for rustc and cargo.
sdk_use_toolchain() {
    RUSTUP_TOOLCHAIN="$(sdk_setting TOOLCHAIN)"
    [ -n "$RUSTUP_TOOLCHAIN" ] || sdk_die "share/breenix-sdk/sdk.env names no TOOLCHAIN"
    export RUSTUP_TOOLCHAIN
}

sdk_find_clang() {
    if [ -n "${BREENIX_CC_CLANG:-}" ]; then
        echo "$BREENIX_CC_CLANG"
        return
    fi
    for _sdk_clang in /opt/homebrew/opt/llvm/bin/clang /usr/local/opt/llvm/bin/clang \
                      /home/linuxbrew/.linuxbrew/opt/llvm/bin/clang clang; do
        if command -v "$_sdk_clang" >/dev/null 2>&1 && "$_sdk_clang" -print-resource-dir >/dev/null 2>&1; then
            echo "$_sdk_clang"
            return
        fi
    done
    sdk_die "no clang whose -print-resource-dir works (install LLVM's clang, or set BREENIX_CC_CLANG)"
}

# The pinned toolchain's gcc-ld directory, whose ld.lld clang links with.
sdk_find_lld_dir() {
    sdk_use_toolchain
    _sdk_sysroot="$(rustc --print sysroot)" || sdk_die "rustc $RUSTUP_TOOLCHAIN does not run (rustup toolchain install $RUSTUP_TOOLCHAIN)"
    _sdk_vv="$(rustc -vV)" || sdk_die "rustc identity is unavailable"
    _sdk_commit="$(printf '%s\n' "$_sdk_vv" | sed -n 's/^commit-hash: //p')"
    [ "$_sdk_commit" = "$(sdk_setting RUSTC_COMMIT)" ] || sdk_die "rustc commit differs from sdk.env"
    _sdk_host="$(printf '%s\n' "$_sdk_vv" | sed -n 's/^host: //p')"
    _sdk_lld="$_sdk_sysroot/lib/rustlib/$_sdk_host/bin/gcc-ld"
    [ -x "$_sdk_lld/ld.lld" ] || sdk_die "the host toolchain has no gcc-ld/ld.lld"
    echo "$_sdk_lld"
}

# sdk_cc ARCH CLANG-ARGUMENTS...: compile and link C for Breenix on ARCH against the
# SDK's headers and libc. Headers come only from include/ and the compiler's own resource
# directory. A command that links (none of -c, -S, -E, -M, -MM, -fsyntax-only, -print-*)
# links a static program with the SDK's linker script and the arch's libc.a, which
# supplies _start; with -r it only combines objects. scripts/breenix-cc is the in-tree
# driver with the same flags.
sdk_cc() {
    _sdk_arch="$1"
    shift
    case "$_sdk_arch" in
        aarch64) _sdk_triple=aarch64-unknown-linux-gnu; _sdk_arch_flags=-mno-outline-atomics ;;
        x86_64) _sdk_triple=x86_64-unknown-linux-gnu; _sdk_arch_flags= ;;
        *) sdk_die "unknown architecture $_sdk_arch" ;;
    esac
    _sdk_cc="$(sdk_find_clang)"
    if [ "$#" = 1 ] && [ "$1" = --version ]; then
        sdk_version_line "$_sdk_arch"
        exec "$_sdk_cc" --version
    fi
    _sdk_resource="$("$_sdk_cc" -print-resource-dir)"
    [ -d "$_sdk_resource/include" ] || sdk_die "$_sdk_resource/include is missing"
    _sdk_linking=1
    _sdk_relocatable=0
    for _sdk_arg in "$@"; do
        case "$_sdk_arg" in
            -c|-S|-E|-M|-MM|-fsyntax-only|-print-*|--print-*|-dump*|--help|-###) _sdk_linking=0 ;;
            -r|-Wl,-r) _sdk_relocatable=1 ;;
        esac
    done
    # shellcheck disable=SC2086
    set -- --no-default-config --sysroot="$SDK" --target="$_sdk_triple" -Ulinux -U__linux -U__linux__ -U__gnu_linux__ -D__breenix__=1 -nostdinc -isystem "$SDK/include" -isystem "$_sdk_resource/include" \
        -fno-stack-protector -fno-pic -fno-pie $_sdk_arch_flags "$@"
    if [ "$_sdk_linking" = 0 ]; then
        exec "$_sdk_cc" "$@"
    fi
    _sdk_lld="$(sdk_find_lld_dir)"
    if [ "$_sdk_relocatable" = 1 ]; then
        exec "$_sdk_cc" -nostdlib -fuse-ld=lld -B "$_sdk_lld" "$@"
    fi
    _sdk_lib="$SDK/lib/$_sdk_arch-breenix"
    [ -f "$_sdk_lib/libc.a" ] || sdk_die "the SDK has no lib/$_sdk_arch-breenix/libc.a"
    exec "$_sdk_cc" -static -nostdlib -fuse-ld=lld -B "$_sdk_lld" \
        -Wl,-T,"$SDK/share/breenix-sdk/ldscripts/breenix.ld" -Wl,--gc-sections -L "$_sdk_lib" \
        "$@" "$_sdk_lib/libc.a"
}
