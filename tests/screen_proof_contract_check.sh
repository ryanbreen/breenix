#!/bin/sh
# Check the contract prose with the repository claim linter.
set -eu
cd "$(dirname "$0")/.."
test -f docs/gui/screen-proof.md
exec python3 scripts/claim-lint.py --files docs/gui/screen-proof.md
