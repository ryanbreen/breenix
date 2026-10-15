#!/bin/sh
# RUSTAPP owns the Rust half of the SDK: the target specs, the fork pin, a prebuilt std
# sysroot for each Breenix target, the vendored libbreenix and the cargo wrapper.
#
# Needs from earlier stages: the pinned fork at $SDK_ROOT/rust-fork/library (its libc patch
# resolving to $SDK_ROOT/libs/libc), lib/aarch64-breenix/libc.a, the soft-float x86_64
# libc.a in $SDK_ROOT/libs/libbreenix-libc/target, share/breenix-sdk/ldscripts/breenix.ld
# and share/breenix-sdk/sdk.env.
set -eu

share=$SDK_STAGE/share/breenix-sdk
rust=$SDK_STAGE/lib/rust
work=$SDK_ROOT/rust-sysroot
fork=$SDK_ROOT/rust-fork/library
install -d "$SDK_STAGE/bin" "$share/libexec" "$share/targets" "$share/vendor" "$rust"

for arch in aarch64 x86_64; do
    install -m 644 "$SDK_ROOT/$arch-breenix.json" "$share/targets/$arch-breenix.json"
done
install -m 644 "$SDK_ROOT/rust-fork.toml" "$share/rust-fork.toml"
fork_commit=$(sed -n 's/^commit = "\([0-9a-f]*\)"$/\1/p' "$SDK_ROOT/rust-fork.toml")
if [ ! -f "$fork/std/Cargo.toml" ]; then
    echo "40-rust: the pinned Rust fork is not at $fork" >&2
    exit 1
fi

export RUSTUP_TOOLCHAIN="$SDK_TOOLCHAIN"
rustc_commit=$(rustc -vV | sed -n 's/^commit-hash: //p')
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_RUSTFLAGS CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR \
    CARGO_BUILD_TARGET RUSTC RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER

# std's flags, separated by 0x1f as CARGO_ENCODED_RUSTFLAGS wants. Paths that must not
# reach the rlibs are remapped general before specific: rustc applies the last match.
sep=$(printf '\037')
flags="-Cmetadata=breenix_std_$fork_commit"
remap() {
    for form in "$1" "$(cd "$1" 2>/dev/null && pwd -P)"; do
        if [ -n "$form" ]; then
            flags="$flags$sep--remap-path-prefix=$form=$2"
        fi
    done
}
remap "$HOME" /home
remap "${CARGO_HOME:-$HOME/.cargo}" /cargo
remap "$(rustc --print sysroot)" "/rustc/$rustc_commit"
remap "$SDK_ROOT" /breenix-sdk-build

# A throwaway crate whose -Zbuild-std build leaves std and its dependencies in deps/.
rm -rf "$work"
mkdir -p "$work/src"
cat > "$work/Cargo.toml" <<'EOF'
[package]
name = "breenix-sdk-sysroot"
version = "0.0.0"
edition = "2021"
publish = false

[profile.release]
panic = "abort"

[workspace]
EOF
echo '#![no_std]' > "$work/src/lib.rs"

for arch in aarch64 x86_64; do
    target=$arch-breenix
    echo "40-rust: building std for $target from fork $fork_commit"
    (
        cd "$work"
        RUST_TARGET_PATH="$share/targets" __CARGO_TESTS_ONLY_SRC_ROOT="$fork" \
            CARGO_ENCODED_RUSTFLAGS="$flags" \
            cargo build --quiet --release --offline --target "$target" \
            -Zbuild-std=std,panic_abort -Zbuild-std-features=compiler-builtins-mem
    )
    lib=$rust/lib/rustlib/$target/lib
    install -d "$lib"
    for rlib in "$work/target/$target/release/deps"/lib*.rlib; do
        case ${rlib##*/} in
        libbreenix_sdk_sysroot-*) ;;
        *) install -m 644 "$rlib" "$lib/" ;;
        esac
    done
    # compiler_builtins carries the libm crate, a port of musl's libm; libc.a supplies
    # Breenix's own math.
    for rlib in "$lib"/libcompiler_builtins-*.rlib; do
        python3 "$SDK_ROOT/scripts/libc-clean-archive.py" --archive "$rlib" --tool-dir "$SDK_LLVM_TOOLS"
    done
done

# Rust on x86_64 is soft-float, so its libc.a is a build of its own, beside its std.
softfloat=$SDK_ROOT/libs/libbreenix-libc/target/x86_64-breenix/release/libc.a
install -m 644 "$softfloat" "$rust/lib/rustlib/x86_64-breenix/lib/libc.a"
python3 "$SDK_ROOT/scripts/libc-clean-archive.py" --archive "$rust/lib/rustlib/x86_64-breenix/lib/libc.a" \
    --tool-dir "$SDK_LLVM_TOOLS"
rm -rf "$work"

libbreenix=$(python3 "$SDK_ROOT/sdk/build/rust-vendor.py" "$SDK_ROOT/libs/libbreenix" "$share/vendor")

env_file=$share/sdk.env
touch "$env_file"
grep -q '^TOOLCHAIN=' "$env_file" || echo "TOOLCHAIN=$SDK_TOOLCHAIN" >> "$env_file"
{
    echo "RUSTC_COMMIT=$rustc_commit"
    echo "FORK=$fork_commit"
    echo "LIBBREENIX=$libbreenix"
} >> "$env_file"

install -m 755 "$SDK_ROOT/sdk/libexec/cargo.sh" "$share/libexec/cargo.sh"
for arch in aarch64 x86_64; do
    cat > "$SDK_STAGE/bin/$arch-breenix-cargo" <<EOF
#!/bin/sh
# cargo for $arch-breenix with the Breenix SDK's prebuilt std; see share/breenix-sdk/libexec/cargo.sh.
self=\$0
while [ -h "\$self" ]; do
    link=\$(readlink "\$self")
    case \$link in /*) self=\$link ;; *) self=\${self%/*}/\$link ;; esac
done
case \$self in */*) dir=\${self%/*} ;; *) dir=. ;; esac
root=\$(cd "\$dir/.." && pwd -P) || exit 1
exec /bin/sh "\$root/share/breenix-sdk/libexec/cargo.sh" "\$root" $arch "\$@"
EOF
    chmod 755 "$SDK_STAGE/bin/$arch-breenix-cargo"
done
