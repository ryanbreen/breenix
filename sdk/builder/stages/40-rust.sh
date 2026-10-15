# Stage 40, rust (RUSTAPP lane): the prebuilt Rust sysroot for each Breenix target, the target specs, rust-fork.toml, the vendored libbreenix and libexec/cargo.sh.
#
#   lib/rust/lib/rustlib/<target>/lib/*.rlib    std and its dependencies, built from the pinned fork
#   share/breenix-sdk/targets/<target>.json     the rustc target specs, verbatim
#   share/breenix-sdk/rust-fork.toml            the fork pin std was built from
#   share/breenix-sdk/vendor/libbreenix-<v>/    libbreenix as a cargo directory source
#   share/breenix-sdk/libexec/cargo.sh          what bin/<arch>-breenix-cargo runs
#
# x86_64's soft-float libc.a, which Rust links, is stage 20's, beside x86_64's std.
. "$SDK_ROOT/sdk/builder/lib.sh"

SHARE="$SDK_STAGE/share/breenix-sdk"
FORK_LIBRARY="$SDK_ROOT/rust-fork/library"
for arch in aarch64 x86_64; do
    sdk_install "$SDK_ROOT/$arch-breenix.json" "share/breenix-sdk/targets/$arch-breenix.json"
done
sdk_install "$SDK_ROOT/rust-fork.toml" share/breenix-sdk/rust-fork.toml
[ -f "$FORK_LIBRARY/std/Cargo.toml" ] || sdk_die "the pinned Rust fork is not at $FORK_LIBRARY"

# A throwaway crate whose -Zbuild-std build leaves std and its dependencies in deps/.
WORK="$SDK_ROOT/rust-sysroot"
rm -rf "$WORK"
mkdir -p "$WORK/src"
cat > "$WORK/Cargo.toml" <<'EOF'
[package]
name = "breenix-sdk-sysroot"
version = "0.0.0"
edition = "2021"
publish = false

[profile.release]
panic = "abort"

[workspace]
EOF
echo '#![no_std]' > "$WORK/src/lib.rs"

for arch in aarch64 x86_64; do
    target="$arch-breenix"
    sdk_say "building std for $target from fork $SDK_FORK_COMMIT"
    (
        export RUST_TARGET_PATH="$SHARE/targets" __CARGO_TESTS_ONLY_SRC_ROOT="$FORK_LIBRARY"
        # cargo asks for no bitcode when nothing in this build uses LTO; an app built with
        # lto = true or "thin" needs it in every sysroot rlib, as rustup's own std carries it.
        sdk_cargo "$WORK" --rustflag "-Cmetadata=breenix_std_$SDK_FORK_COMMIT" --rustflag -Cembed-bitcode=yes -- \
            build --quiet --release --target "$target" \
            -Zbuild-std=std,panic_abort -Zbuild-std-features=compiler-builtins-mem
    )
    lib="lib/rust/lib/rustlib/$target/lib"
    for rlib in "$WORK/target/$target/release/deps"/lib*.rlib; do
        case "$(basename "$rlib")" in
            libbreenix_sdk_sysroot-*) ;;
            *) sdk_install "$rlib" "$lib/$(basename "$rlib")" ;;
        esac
    done
    # The pinned fork builds none of the libm crate's math (a port of musl's) into compiler_builtins
    # for Breenix; libc.a has Breenix's own. Stripping objects would leave the math in an rlib's
    # metadata and bitcode, so a std that carries any of it stops the build instead.
    sdk_no_libm_math "$SDK_STAGE/$lib"/*.rlib
done
[ -f "$SDK_STAGE/lib/rust/lib/rustlib/x86_64-breenix/lib/libc.a" ] ||
    sdk_die "stage 20 left no soft-float libc.a beside x86_64-breenix's std"
rm -rf "$WORK"

python3 "$SDK_ROOT/sdk/builder/rust-vendor.py" "$SDK_ROOT/libs/libbreenix" "$SHARE/vendor" > /dev/null
sdk_install "$SDK_ROOT/sdk/libexec/cargo.sh" share/breenix-sdk/libexec/cargo.sh
