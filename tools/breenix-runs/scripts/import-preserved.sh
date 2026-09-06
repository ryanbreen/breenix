#!/usr/bin/env bash
set -euo pipefail
PACKAGE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$PACKAGE_DIR/../.." && pwd)"
BIN_DIR="$(cd "$PACKAGE_DIR" && swift build --show-bin-path -c release)"
paths=()
while IFS= read -r -d '' directory; do
    paths+=("$directory")
done < <(find "$REPO_DIR/docs/planning/green-program" -type d -name serials -print0)
if [ "${#paths[@]}" -eq 0 ]; then
    echo "Total: imported=0 skipped=0 existing=0 (no green-program serials directories)"
    exit 0
fi
"$BIN_DIR/breenix-runs" import "${paths[@]}"
