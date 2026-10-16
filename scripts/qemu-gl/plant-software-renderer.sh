#!/usr/bin/env bash
# Builds the planted negative control for the GL QEMU's host renderer check: a copy of the
# installed QEMU-GL.app whose executable is byte for byte the installed one and whose EGL and
# GLESv2 frameworks are Homebrew Mesa's, so the same binary renders on llvmpipe.
#
#   scripts/qemu-gl/plant-software-renderer.sh DIR
#
# Prints the planted executable. Boot it with
#   BREENIX_QEMU_GL_BINARY=<that path> EGL_PLATFORM=surfaceless \
#       scripts/boot-interactive.sh --gpu-gl --no-display ...
# and scripts/check-host-renderer.py refuses the run, naming llvmpipe. Needs Homebrew's mesa.
set -euo pipefail

DIR="${1:?usage: $0 DIR}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="$("$ROOT/scripts/build-qemu-gl.sh" --print-path)" || { echo "run scripts/build-qemu-gl.sh first" >&2; exit 1; }
APP="${BIN%/Contents/MacOS/qemu-system-aarch64}"
MESA="$(brew --prefix mesa)/lib"
[ -f "$MESA/libEGL.1.dylib" ] && [ -f "$MESA/libGLESv2.2.dylib" ] || { echo "needs Homebrew's mesa" >&2; exit 1; }

mkdir -p "$DIR"
[ ! -e "$DIR/QEMU-GL.app" ] || { echo "$DIR/QEMU-GL.app already exists" >&2; exit 1; }
cp -Rp "$APP" "$DIR/QEMU-GL.app"
FW="$DIR/QEMU-GL.app/Contents/Frameworks"
cc -dynamiclib -O2 -Wall -Werror -o "$FW/EGL.framework/Versions/A/EGL" \
    -install_name @rpath/EGL.framework/Versions/A/EGL \
    "$ROOT/scripts/qemu-gl/software-egl.c" -Wl,-reexport_library,"$MESA/libEGL.1.dylib"
cp "$MESA/libGLESv2.2.dylib" "$FW/GLESv2.framework/Versions/A/GLESv2"
codesign --force --sign - "$FW/EGL.framework/Versions/A/EGL" "$FW/GLESv2.framework/Versions/A/GLESv2" 2>/dev/null
cmp "$BIN" "$DIR/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64"
echo "$DIR/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64"
