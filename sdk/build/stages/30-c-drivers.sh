# Stage 30, C drivers (TOOLCHAIN lane): bin/<arch>-breenix-cc, the linker script they link
# with, and bin/<arch>-breenix-cargo (the entry the RUSTAPP lane's libexec/cargo.sh serves).
. "$SDK_ROOT/sdk/build/lib.sh"

for arch in aarch64 x86_64; do
    sdk_install "$SDK_ROOT/sdk/bin/$arch-breenix-cc" "bin/$arch-breenix-cc"
    sdk_install "$SDK_ROOT/sdk/bin/$arch-breenix-cargo" "bin/$arch-breenix-cargo"
done
sdk_install "$SDK_ROOT/userspace/programs/linker.ld" share/breenix-sdk/ldscripts/breenix.ld
