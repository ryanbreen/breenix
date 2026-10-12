#!/usr/bin/env bash
# Prints the path of a qemu-system binary that draws its Cocoa window at one guest pixel per point.
#
# QEMU's Cocoa display marks itself high-resolution capable, so on a Retina screen a 1280x800 guest
# fills a 640x400-point window, a quarter of the area a VMware or Parallels window of the same guest
# covers. QEMU 11 has no option to scale it. An app bundle whose Info.plist says
# NSHighResolutionCapable=false makes macOS draw it at 1x, doubling the window. The bundle holds a
# copy of the Homebrew binary (macOS finds a bundle from the executable's real path, so a link would
# not do) and is rebuilt whenever that binary changes. Prints the plain binary when anything fails.
#
# Usage: qemu-1x-app.sh qemu-system-aarch64
set -u
NAME="${1:?usage: qemu-1x-app.sh qemu-system-<arch>}"
SOURCE="$(command -v "$NAME" 2>/dev/null)" || { echo "$NAME"; exit 0; }
REAL="$(realpath "$SOURCE" 2>/dev/null)" || { echo "$SOURCE"; exit 0; }
APP="${BREENIX_QEMU_APP_DIR:-$HOME/Library/Caches/breenix}/QEMU-1x-$NAME.app"
BIN="$APP/Contents/MacOS/$NAME"
STAMP="$APP/Contents/source"
WANT="$REAL $(stat -f '%z %m' "$REAL" 2>/dev/null)"
if [ ! -x "$BIN" ] || [ "$(cat "$STAMP" 2>/dev/null)" != "$WANT" ]; then
    TMP="$APP.tmp.$$"
    rm -rf "$TMP"
    if mkdir -p "$TMP/Contents/MacOS" && cp "$REAL" "$TMP/Contents/MacOS/$NAME" && cat > "$TMP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>$NAME</string>
    <key>CFBundleIdentifier</key><string>org.breenix.qemu-1x.$NAME</string>
    <key>CFBundleName</key><string>QEMU</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>NSHighResolutionCapable</key><false/>
</dict>
</plist>
PLIST
    then
        echo "$WANT" > "$TMP/Contents/source"
        rm -rf "$APP" && mv "$TMP" "$APP"
    else
        rm -rf "$TMP"
        echo "$SOURCE"
        exit 0
    fi
fi
# Keep App Nap off the bundle (#1292). Being an app, the bundle is napped once its window has been out
# of sight for a while: 36-58 s into a boot macOS dropped QEMU from priority 46 to 4 and coalesced its
# timers, so QEMU raised the guest's 1 ms ticks about 30 times a second instead of about 500, and ARM64
# time cases ran 20-90 ms late. The plain binary is not an app and was not napped. The user default
# NSAppSleepDisabled is what AppKit reads; LSAppNapIsDisabled in Info.plist and `taskpolicy -l 0 -t 0`
# did not stop the demotion. If the default cannot be written, the plain binary is used instead.
if ! defaults write "org.breenix.qemu-1x.$NAME" NSAppSleepDisabled -bool YES 2>/dev/null; then
    echo "qemu-1x-app.sh: could not disable App Nap for org.breenix.qemu-1x.$NAME; using the plain $NAME" >&2
    echo "$SOURCE"
    exit 0
fi
echo "$BIN"
