# Stage 10, layout (TOOLCHAIN lane): the SDK's directories, VERSION, its settings and the
# shell code its bin/ entries share.
. "$SDK_ROOT/sdk/build/lib.sh"

for dir in bin include lib share/breenix-sdk/libexec share/doc/breenix-sdk; do
    mkdir -p "$SDK_STAGE/$dir"
done
echo "breenix-sdk $SDK_VERSION $SDK_COMMIT" > "$SDK_STAGE/VERSION"

LIBBREENIX="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$SDK_ROOT/libs/libbreenix/Cargo.toml" | head -1)"
[ -n "$LIBBREENIX" ] || sdk_die "libs/libbreenix/Cargo.toml names no version"
cat > "$SDK_STAGE/share/breenix-sdk/sdk.env" <<ENV
TOOLCHAIN=$SDK_TOOLCHAIN
RUSTC_COMMIT=$SDK_RUSTC_COMMIT
FORK=$SDK_FORK_COMMIT
LIBBREENIX=$LIBBREENIX
ENV
sdk_install "$SDK_ROOT/sdk/libexec/sdk-common.sh" share/breenix-sdk/libexec/sdk-common.sh
