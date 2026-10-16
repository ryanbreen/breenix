#!/usr/bin/env bash
# Builds the GL-capable qemu-system-aarch64 that ARM64 Breenix boots on a Mac use for
# virtio-gpu-gl-pci, and installs it beside the Homebrew QEMU without replacing it.
#
#   scripts/build-qemu-gl.sh               build (if this pin is not installed) and install
#   scripts/build-qemu-gl.sh --rebuild     build and install even if this pin is installed
#   scripts/build-qemu-gl.sh --print-path  print the installed binary, or exit 1 if absent
#
# The host stack is UTM's matched set, pinned at UTM's dependency manifest
# (patches/sources) at UTM_COMMIT: QEMU 10.0.12-utm with UTM's patch set, WebKit's ANGLE
# built for its Metal backend, UTM's libepoxy and virglrenderer forks, and the SPICE
# server. UTM builds QEMU as a library for its app; this builds the same patched source
# as a command-line executable with Cocoa enabled, so one binary serves headless runs
# (-spice gl=es with -display none) and the interactive window (-display cocoa,gl=es).
# On top of UTM's patches it applies scripts/qemu-gl/host-renderer-attestation.patch.
#
# Libraries UTM also builds from source that carry no graphics code (glib, pixman,
# libslirp, OpenSSL, libjpeg, Opus, libpng) come from Homebrew. virglrenderer is built
# without Venus and Neptune (Vulkan and Direct3D), which the VirGL path does not use.
#
# The result is an app bundle, <cache>/qemu-gl/<PIN>/QEMU-GL.app, holding the executable,
# its libraries and ANGLE's EGL and GLESv2 frameworks, ad-hoc signed with the hypervisor
# entitlement. Run it with ANGLE_DEFAULT_PLATFORM=metal (UTM's default backend);
# without it ANGLE picks its OpenGL backend. Downloads are cached beside the bundle; the
# build tree is /tmp/breenix-qemu-gl-build, emptied at the start of each build, so one
# build runs at a time. Builds run at background priority with at most
# BREENIX_QEMU_GL_JOBS (default 4) jobs.
#
# Host prerequisites: Xcode (with the macOS SDK), git, curl, meson, ninja, pkg-config and
# Homebrew's glib, pixman, libslirp, openssl@3, jpeg-turbo, opus and libpng.
set -euo pipefail

UTM_COMMIT=4d73aaf0d284f1c22e3e1c1072e729988e134268
QEMU_URL=https://github.com/utmapp/qemu/releases/download/v10.0.12-utm/qemu-10.0.12-utm.tar.xz
QEMU_SHA256=7c9605290b34152debb842e965a55d2e4fbba4793e536ab677b4027e5dc0ba1a
QEMU_PATCH_SHA256=84eee5ca4a512571478029c75bf31f89a3768f476b0f79457e2445c002ed4c13
SPICE_PROTOCOL_URL=https://www.spice-space.org/download/releases/spice-protocol-0.14.4.tar.xz
SPICE_PROTOCOL_SHA256=04ffba610d9fd441cfc47dfaa135d70096e60b1046d2119d8db2f8ea0d17d912
SPICE_SERVER_URL=https://www.spice-space.org/download/releases/spice-server/spice-0.14.3.tar.bz2
SPICE_SERVER_SHA256=551d4be4a07667cf0543f3c895beb6da8a93ef5a9829f2ae47817be5e616a114
SPICE_PATCH_SHA256=251638a358cd65529a349f3a33d14bb2cb64d8631ea73521f0be2720b3e39a09
WEBKIT_REPO=https://github.com/utmapp/WebKit.git
WEBKIT_COMMIT=ed78ab6e1a37f4f11583a0bd038f22ec91f3ff10
EPOXY_REPO=https://github.com/utmapp/libepoxy.git
EPOXY_COMMIT=bf98587477fe68d07b93319ece7b40a7d0e2eabe
VIRGL_REPO=https://github.com/utmapp/virglrenderer.git
VIRGL_COMMIT=451505ad0aa449290f2c0672da6877d57ee770e5
DTC_REPO=https://gitlab.com/qemu-project/dtc.git
DTC_COMMIT=b6910bec11614980a21e46fbccc35934b671bd81
# Bump the suffix whenever the recipe or the Breenix patch changes, so a stale build is not reused.
PIN=10.0.12-utm-4d73aaf0-b1

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE="${BREENIX_QEMU_GL_DIR:-$HOME/Library/Caches/breenix/qemu-gl}"
DEST="$CACHE/$PIN/QEMU-GL.app"
BIN="$DEST/Contents/MacOS/qemu-system-aarch64"
BUNDLE_ID=org.breenix.qemu-gl

die() { echo "build-qemu-gl: $*" >&2; exit 1; }

REBUILD=0
case "${1:-}" in
    --print-path)
        [ -x "$BIN" ] || exit 1
        echo "$BIN"
        exit 0
        ;;
    --rebuild) REBUILD=1 ;;
    "") ;;
    *) echo "usage: $0 [--rebuild | --print-path]" >&2; exit 2 ;;
esac
[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] || die "builds only on an Apple silicon Mac"
if [ "$REBUILD" -eq 0 ] && [ -x "$BIN" ] && codesign --verify --strict "$DEST" 2>/dev/null; then
    echo "build-qemu-gl: $PIN is installed: $BIN"
    exit 0
fi
# Builds never compete with boots or the operator: rerun at background priority.
if [ -z "${BREENIX_QEMU_GL_BACKGROUND:-}" ]; then
    export BREENIX_QEMU_GL_BACKGROUND=1
    exec taskpolicy -b "$0" "$@"
fi

JOBS="${BREENIX_QEMU_GL_JOBS:-4}"
WORK=/tmp/breenix-qemu-gl-build
DOWNLOADS="$CACHE/downloads"
SRC="$WORK/src"
PREFIX="$WORK/prefix"
export MACOSX_DEPLOYMENT_TARGET=26.0

for tool in xcodebuild git curl meson ninja pkg-config shasum codesign install_name_tool rsync; do
    command -v "$tool" >/dev/null || die "missing $tool"
done
PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig:$PREFIX/share/pkgconfig"
if command -v brew >/dev/null; then
    PKG_CONFIG_PATH="$PKG_CONFIG_PATH:$(brew --prefix openssl@3)/lib/pkgconfig"
fi
export PKG_CONFIG_PATH
for pc in glib-2.0 gio-2.0 pixman-1 slirp openssl libjpeg opus libpng; do
    pkg-config --exists "$pc" || die "missing development package $pc (Homebrew: glib pixman libslirp openssl@3 jpeg-turbo opus libpng)"
done

step() { echo "==> $* ($(date '+%H:%M:%S'))"; }

# fetch URL SHA256: downloads into $DOWNLOADS once and checks the digest every time.
fetch() {
    local file="$DOWNLOADS/${1##*/}"
    if [ ! -f "$file" ]; then
        curl -fL --retry 3 -o "$file.part" "$1"
        mv "$file.part" "$file"
    fi
    echo "$2  $file" | shasum -a 256 -c --status || die "checksum mismatch for $file (delete it to download it again)"
}

# utm_patch NAME SHA256: one of UTM's patches at the pinned commit.
utm_patch() {
    fetch "https://raw.githubusercontent.com/utmapp/UTM/$UTM_COMMIT/patches/$1" "$2"
}

# git_pin REPO COMMIT DIR [SPARSE_DIR...]: a shallow checkout of exactly COMMIT.
git_pin() {
    local repo="$1" commit="$2" dir="$3"
    shift 3
    git init -q "$dir"
    git -C "$dir" remote add origin "$repo"
    [ $# -eq 0 ] || git -C "$dir" sparse-checkout set --cone "$@"
    git -C "$dir" fetch -q --depth 1 --filter=blob:none origin "$commit"
    git -C "$dir" switch -q --detach FETCH_HEAD
    [ "$(git -C "$dir" rev-parse HEAD)" = "$commit" ] || die "$repo is not at $commit"
}

# unpack ARCHIVE: its tree under $SRC, printed.
unpack() {
    local name="${1##*/}"
    tar -xf "$1" -C "$SRC"
    echo "$SRC/${name%.tar.*}"
}

# meson_build DIR [OPTION...]: configure, build and install into $PREFIX.
meson_build() {
    local dir="$1"
    shift
    CFLAGS="-I$PREFIX/include" meson setup "$dir/build-breenix" "$dir" --prefix="$PREFIX" \
        --libdir=lib --buildtype=release "$@"
    ninja -C "$dir/build-breenix" -j "$JOBS"
    meson install -C "$dir/build-breenix" >/dev/null
}

START=$(date +%s)
rm -rf /tmp/breenix-qemu-gl-build
mkdir -p "$SRC" "$PREFIX/lib" "$PREFIX/include" "$DOWNLOADS"

step "fetching pinned sources"
fetch "$QEMU_URL" "$QEMU_SHA256"
fetch "$SPICE_PROTOCOL_URL" "$SPICE_PROTOCOL_SHA256"
fetch "$SPICE_SERVER_URL" "$SPICE_SERVER_SHA256"
utm_patch qemu-10.0.12-utm.patch "$QEMU_PATCH_SHA256"
utm_patch spice-0.14.3.patch "$SPICE_PATCH_SHA256"
git_pin "$WEBKIT_REPO" "$WEBKIT_COMMIT" "$SRC/WebKit" Source/ThirdParty/ANGLE Configurations Tools/ccache
git_pin "$EPOXY_REPO" "$EPOXY_COMMIT" "$SRC/libepoxy"
git_pin "$VIRGL_REPO" "$VIRGL_COMMIT" "$SRC/virglrenderer"

step "building ANGLE (Metal and OpenGL backends)"
# ANGLE keeps UTM's deployment target: from 26.0 WebKit's configuration defines
# _LIBCPP_ENABLE_ASSERTIONS, which the macOS 27 SDK's libc++ rejects.
(
    cd "$SRC/WebKit/Source/ThirdParty/ANGLE"
    env -i PATH="/usr/bin:/bin:/usr/sbin:/sbin" HOME="$HOME" xcodebuild archive -quiet \
        -archivePath ANGLE -scheme ANGLE -sdk macosx -arch arm64 -configuration Release \
        -derivedDataPath "$WORK/angle-derived" -jobs "$JOBS" \
        WEBCORE_LIBRARY_DIR=/usr/local/lib NORMAL_UMBRELLA_FRAMEWORKS_DIR= \
        CODE_SIGNING_ALLOWED=NO GCC_TREAT_WARNINGS_AS_ERRORS=NO \
        MACOSX_DEPLOYMENT_TARGET=12.0
    rsync -a ANGLE.xcarchive/Products/usr/local/lib/ "$PREFIX/lib/"
    rsync -a include/ "$PREFIX/include/"
)

step "building libepoxy"
meson_build "$SRC/libepoxy" -Dtests=false -Dglx=no -Degl=yes

step "building virglrenderer"
meson_build "$SRC/virglrenderer" -Dtests=false -Dcheck-gl-errors=false -Dvenus=false -Dneptune=false

step "building the SPICE server"
meson_build "$(unpack "$DOWNLOADS/spice-protocol-0.14.4.tar.xz")"
SPICE_DIR="$(unpack "$DOWNLOADS/spice-0.14.3.tar.bz2")"
patch -s -d "$SPICE_DIR" -p1 < "$DOWNLOADS/spice-0.14.3.patch"
meson_build "$SPICE_DIR" -Dlz4=false -Dsasl=false -Dmanual=false -Dgstreamer=no -Dsmartcard=disabled

step "building qemu-system-aarch64"
QEMU_DIR="$(unpack "$DOWNLOADS/qemu-10.0.12-utm.tar.xz")"
patch -s -d "$QEMU_DIR" -p1 < "$DOWNLOADS/qemu-10.0.12-utm.patch"
patch -s -d "$QEMU_DIR" -p1 < "$ROOT/scripts/qemu-gl/host-renderer-attestation.patch"
# libfdt from the revision QEMU's own wrap pins; the release tarball does not carry it.
git_pin "$DTC_REPO" "$DTC_COMMIT" "$QEMU_DIR/subprojects/dtc"
grep -q "^revision = $DTC_COMMIT\$" "$QEMU_DIR/subprojects/dtc.wrap" || die "QEMU's dtc.wrap no longer pins $DTC_COMMIT"
mkdir -p "$QEMU_DIR/build-breenix"
(
    cd "$QEMU_DIR/build-breenix"
    ../configure --target-list=aarch64-softmmu --without-default-features --disable-download \
        --enable-hvf --enable-tcg --enable-cocoa --enable-opengl --enable-virglrenderer \
        --enable-spice --enable-spice-protocol --enable-slirp --enable-fdt=internal \
        --enable-pixman --enable-png --disable-docs --disable-debug-info \
        --extra-cflags="-I$PREFIX/include"
    ninja -j "$JOBS" qemu-system-aarch64
)

step "assembling and signing QEMU-GL.app"
STAGE="$WORK/stage"
APP="$STAGE/QEMU-GL.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/share/qemu"
cp "$QEMU_DIR/build-breenix/qemu-system-aarch64" "$APP/Contents/MacOS/"
cp -R "$QEMU_DIR/pc-bios/keymaps" "$APP/Contents/share/qemu/"
cp "$QEMU_DIR/pc-bios/efi-virtio.rom" "$APP/Contents/share/qemu/"
plist() {
    cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
$1
</dict>
</plist>
PLIST
}
LIBS="libepoxy.0.dylib libvirglrenderer.1.dylib libspice-server.1.dylib"
for lib in $LIBS; do
    cp "$PREFIX/lib/$lib" "$APP/Contents/Frameworks/"
    install_name_tool -id "@rpath/$lib" "$APP/Contents/Frameworks/$lib" 2>/dev/null
done
# libepoxy dlopens EGL.framework/... and GLESv2.framework/..., which dyld resolves through
# the executable's rpath, as UTM's fixup.sh lays them out.
for fw in EGL GLESv2; do
    dir="$APP/Contents/Frameworks/$fw.framework"
    mkdir -p "$dir/Versions/A/Resources"
    cp "$PREFIX/lib/lib$fw.dylib" "$dir/Versions/A/$fw"
    install_name_tool -id "@rpath/$fw.framework/Versions/A/$fw" "$dir/Versions/A/$fw" 2>/dev/null
    plist "    <key>CFBundleExecutable</key><string>$fw</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID.$fw</string>
    <key>CFBundlePackageType</key><string>FMWK</string>
    <key>CFBundleShortVersionString</key><string>1.0</string>
    <key>CFBundleVersion</key><string>1</string>" > "$dir/Versions/A/Resources/Info.plist"
    ln -sfh A "$dir/Versions/Current"
    ln -sfh Versions/Current/Resources "$dir/Resources"
    ln -sfh "Versions/Current/$fw" "$dir/$fw"
done
for file in "$APP/Contents/MacOS/qemu-system-aarch64" "$APP"/Contents/Frameworks/*.dylib; do
    for lib in $LIBS; do
        install_name_tool -change "$PREFIX/lib/$lib" "@rpath/$lib" "$file" 2>/dev/null
    done
done
install_name_tool -add_rpath @executable_path/../Frameworks "$APP/Contents/MacOS/qemu-system-aarch64" 2>/dev/null
# The window is drawn at one guest pixel per point, as scripts/qemu-1x-app.sh does for Homebrew's QEMU.
plist "    <key>CFBundleExecutable</key><string>qemu-system-aarch64</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleName</key><string>QEMU GL</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$PIN</string>
    <key>NSHighResolutionCapable</key><false/>" > "$APP/Contents/Info.plist"
for file in "$APP/Contents/MacOS/qemu-system-aarch64" "$APP"/Contents/Frameworks/*.dylib; do
    if otool -L "$file" | tail -n +2 | grep -F "$WORK"; then
        die "$file still loads from the build directory"
    fi
done
for item in "$APP"/Contents/Frameworks/*.framework "$APP"/Contents/Frameworks/*.dylib; do
    codesign --force --sign - "$item"
done
codesign --force --sign - --entitlements "$QEMU_DIR/accel/hvf/entitlements.plist" "$APP"
codesign --verify --strict --deep "$APP"
codesign -d --entitlements - "$APP/Contents/MacOS/qemu-system-aarch64" 2>/dev/null \
    | grep -q com.apple.security.hypervisor || die "the hypervisor entitlement is missing"
cat > "$STAGE/PINS" <<PINS
pin $PIN
utm $UTM_COMMIT
qemu $QEMU_URL sha256:$QEMU_SHA256
qemu-patch qemu-10.0.12-utm.patch sha256:$QEMU_PATCH_SHA256
breenix-patch host-renderer-attestation.patch sha256:$(shasum -a 256 "$ROOT/scripts/qemu-gl/host-renderer-attestation.patch" | cut -d' ' -f1)
angle $WEBKIT_REPO $WEBKIT_COMMIT
libepoxy $EPOXY_REPO $EPOXY_COMMIT
virglrenderer $VIRGL_REPO $VIRGL_COMMIT
dtc $DTC_REPO $DTC_COMMIT
spice-protocol $SPICE_PROTOCOL_URL sha256:$SPICE_PROTOCOL_SHA256
spice-server $SPICE_SERVER_URL sha256:$SPICE_SERVER_SHA256
spice-patch spice-0.14.3.patch sha256:$SPICE_PATCH_SHA256
PINS
# A replaced install moves into the build tree, which the next build empties.
[ ! -e "$CACHE/$PIN" ] || mv "$CACHE/$PIN" "$WORK/replaced"
mv "$STAGE" "$CACHE/$PIN"
# Keep App Nap off the bundle, as scripts/qemu-1x-app.sh does for QEMU-1x (#1292).
defaults write "$BUNDLE_ID" NSAppSleepDisabled -bool YES
END=$(date +%s)
echo "build-qemu-gl: installed $BIN"
echo "build-qemu-gl: built in $((END - START)) s; bundle $(du -sh "$DEST" | cut -f1); executable $(du -h "$BIN" | cut -f1)"
