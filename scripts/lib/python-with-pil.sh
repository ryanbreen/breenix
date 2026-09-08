#!/usr/bin/env bash
# Source this file to select a Python interpreter that can load Pillow.
# BREENIX_PYTHON is a preferred executable (not a command with arguments).
breenix_resolve_python_with_pil() {
    local candidate resolved
    local -a candidates=()
    [ -z "${BREENIX_PYTHON:-}" ] || candidates+=("$BREENIX_PYTHON")
    candidates+=(python3 /opt/homebrew/bin/python3 /usr/local/bin/python3 /usr/bin/python3)
    for candidate in "${candidates[@]}"; do
        [ -n "$candidate" ] || continue
        resolved="$(command -v "$candidate" 2>/dev/null)" || continue
        if "$resolved" -c 'from PIL import Image, ImageChops, ImageDraw' >/dev/null 2>&1; then
            export BREENIX_PYTHON="$resolved"
            return 0
        fi
    done
    echo "ERROR: no Python with Pillow found (tried: ${candidates[*]}); install Pillow or set BREENIX_PYTHON to an interpreter with Pillow" >&2
    return 1
}
breenix_resolve_python_with_pil
