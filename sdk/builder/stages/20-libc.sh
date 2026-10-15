# Stage 20, C library (TOOLCHAIN lane): Breenix's clean-room libc.a for each target, the
# empty libm/libpthread/librt archives C programs name, and the C headers.
#
#   lib/aarch64-breenix/libc.a                         aarch64 (C and Rust)
#   lib/x86_64-breenix/libc.a                          x86_64, SysV SSE float ABI (C)
#   lib/rust/lib/rustlib/x86_64-breenix/lib/libc.a     x86_64, soft-float (Rust only)
. "$SDK_ROOT/sdk/builder/lib.sh"

LIBC="$SDK_ROOT/libs/libbreenix-libc"
# core and compiler_builtins come from the pinned fork, as std's do: its compiler_builtins builds
# none of the libm crate's math (ported from a foreign C library) for Breenix. The cleaner then removes nothing,
# and anything of that math left in an archive, even a source path in its data, stops the build.
build_libc() {
    sdk_say "building libbreenix-libc for $1"
    (
        export __CARGO_TESTS_ONLY_SRC_ROOT="$SDK_ROOT/rust-fork/library"
        sdk_cargo "$LIBC" -- build --release --target "$SDK_ROOT/$1.json"
    )
    sdk_clean_archive "$LIBC/target/$1/release/libc.a"
    python3 "$SDK_ROOT/scripts/libc-symbol-audit.py" --arch "${1%%-*}" --archive "$LIBC/target/$1/release/libc.a"
    sdk_no_libm_math "$LIBC/target/$1/release/libc.a"
}

build_libc aarch64-breenix
build_libc x86_64-breenix-c
build_libc x86_64-breenix
sdk_install "$LIBC/target/aarch64-breenix/release/libc.a" lib/aarch64-breenix/libc.a
sdk_install "$LIBC/target/x86_64-breenix-c/release/libc.a" lib/x86_64-breenix/libc.a
sdk_install "$LIBC/target/x86_64-breenix/release/libc.a" lib/rust/lib/rustlib/x86_64-breenix/lib/libc.a
for arch in aarch64 x86_64; do
    for stub in libm.a libpthread.a librt.a; do
        sdk_install "$LIBC/stublibs/$stub" "lib/$arch-breenix/$stub"
    done
done

mkdir -p "$SDK_STAGE/include"
cp -R "$LIBC/include/." "$SDK_STAGE/include/"
