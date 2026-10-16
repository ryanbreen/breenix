#!/usr/bin/env bash
# Builds the planted negative control for the GL QEMU's host renderer check: a copy of the
# installed QEMU-GL.app whose QEMU executable is byte for byte the installed one and whose ANGLE
# renders on Apple's software OpenGL renderer.
#
#   scripts/qemu-gl/plant-software-renderer.sh DIR
#
# Prints the planted executable. Boot it with ANGLE's OpenGL backend:
#   BREENIX_QEMU_GL_BINARY=<that path> ANGLE_DEFAULT_PLATFORM=gl \
#       scripts/boot-interactive.sh --gpu-gl ...
# and scripts/check-host-renderer.py refuses the run, naming the software renderer.
#
# ANGLE opens OpenGL.framework by its path; in the copy that path names
# scripts/qemu-gl/software-cgl.c instead, which re-exports OpenGL.framework and asks
# every pixel format for the generic software renderer.
set -euo pipefail

DIR="${1:?usage: $0 DIR}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="$("$ROOT/scripts/build-qemu-gl.sh" --print-path)" || { echo "run scripts/build-qemu-gl.sh first" >&2; exit 1; }
APP="${BIN%/Contents/MacOS/qemu-system-aarch64}"

mkdir -p "$DIR"
[ ! -e "$DIR/QEMU-GL.app" ] || { echo "$DIR/QEMU-GL.app already exists" >&2; exit 1; }
cp -Rp "$APP" "$DIR/QEMU-GL.app"
FW="$DIR/QEMU-GL.app/Contents/Frameworks"
cc -dynamiclib -O2 -Wall -Werror -Wno-deprecated-declarations -o "$FW/OpenGLSoftware.dylib" \
    -install_name @rpath/OpenGLSoftware.dylib "$ROOT/scripts/qemu-gl/software-cgl.c" \
    -Wl,-reexport_framework,OpenGL
python3 - "$FW/GLESv2.framework/Versions/A/GLESv2" <<'PY'
import sys
path = sys.argv[1]
data = open(path, "rb").read()
old = b"/System/Library/Frameworks/OpenGL.framework/OpenGL\0"
new = b"@loader_path/../../../OpenGLSoftware.dylib\0"
if data.count(old) != 1:
    sys.exit("ANGLE does not name OpenGL.framework exactly once")
open(path, "wb").write(data.replace(old, new.ljust(len(old), b"\0")))
PY
codesign --force --sign - "$FW/OpenGLSoftware.dylib" "$FW/GLESv2.framework/Versions/A/GLESv2" 2>/dev/null
cmp "$BIN" "$DIR/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64"
echo "$DIR/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64"
